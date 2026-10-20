//! The brief: one snapshot folded into a session-start orientation.
//!
//! `--json` is the API and carries full text; the rendered form goes
//! through a template (`.peat/brief.tmpl` overrides the embedded default)
//! which owns all display-time clipping.

use std::collections::HashMap;

use fold::pipeline::Scored;

use crate::db::Db;
use crate::event::EventId;
use crate::ladder;
use crate::pipeline::{DAY_MS, DayStats, ObsRow, SessStats};
use crate::transcript::date_label;
use crate::ui::{self, age_label, short_path, short_sess};

/// Reciprocal-rank-fusion constant (value from the original RRF paper).
const RRF_K: f64 = 60.0;

/// Fuse keyword and vector hit lists by reciprocal rank — the one ranking
/// rule, shared by `recall` and the brief's `relevant` section. Sorted
/// best-first with a deterministic id tie-break.
pub fn rrf(kw: &[Scored<f64, EventId>], vec: &[Scored<f32, EventId>]) -> Vec<(EventId, f64)> {
    let mut fused: HashMap<EventId, f64> = HashMap::new();
    for (rank, hit) in kw.iter().enumerate() {
        *fused.entry(hit.val.clone()).or_default() += 1.0 / (RRF_K + rank as f64 + 1.0);
    }
    for (rank, hit) in vec.iter().enumerate() {
        *fused.entry(hit.val.clone()).or_default() += 1.0 / (RRF_K + rank as f64 + 1.0);
    }
    let mut fused: Vec<(EventId, f64)> = fused.into_iter().collect();
    fused.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    fused
}

/// The session a resuming agent should read as "last": the newest closed
/// session (one with a final message) whose cwd lies under `desk`. Several
/// desks share one ledger, so the newest closed session overall is often
/// another seat's; that one is the fallback only when this desk has none,
/// flagged `false` so it is never presented as this desk's own. `sess` is
/// newest-first.
pub fn last_session<'a>(
    sess: &'a [(String, SessStats)],
    desk: &std::path::Path,
) -> Option<(&'a SessStats, bool)> {
    let mut closed = sess
        .iter()
        .map(|(_, s)| s)
        .filter(|s| !s.final_msg.is_empty());
    let newest = closed.clone().next()?;
    // the desk comes from getcwd (symlinks resolved); a recorded cwd may
    // name a symlinked desk (`murail-dev -> murail-2a`), so resolve it too
    let under = |cwd: &str| {
        let p = std::path::Path::new(cwd);
        p.starts_with(desk) || p.canonicalize().is_ok_and(|c| c.starts_with(desk))
    };
    match closed.find(|s| under(&s.cwd)) {
        Some(s) => Some((s, true)),
        None => Some((newest, false)),
    }
}

#[derive(serde::Serialize)]
pub struct Brief {
    pub today: String,
    active: Vec<serde_json::Value>,
    days: Vec<serde_json::Value>,
    /// the temporal ladder: the rest of the past, geometrically coarser,
    /// every band carrying the command that descends into it
    further: Vec<ladder::Band>,
    last_session: Option<serde_json::Value>,
    files: Vec<serde_json::Value>,
    relevant: Vec<serde_json::Value>,
}

/// Assemble the brief from one snapshot of the ledger. `desk` is the
/// directory this desk works in, so "last session" means this desk's;
/// `offset_ms` is the reader's UTC offset: days are the reader's local
/// days (see `ladder::local_days`); `calling` is the running session,
/// whose own rows `relevant` ranks after everyone else's.
pub fn assemble(
    st: &Db,
    query: &str,
    now: u64,
    offset_ms: i64,
    budget: usize,
    desk: &std::path::Path,
    calling: Option<String>,
) -> Brief {
    st.rtx(
        |(days, files, (kw, vec, texts), (subjects, evidence), sessions, _)| {
            let today_bucket = ladder::local_day(now, offset_ms);

            // ---- day digest: the 3 most recent non-empty days
            let mut day_rows: Vec<(u64, DayStats)> = ladder::local_days(days.iter(), offset_ms)
                .into_iter()
                .collect();
            day_rows.sort_by_key(|(d, _)| std::cmp::Reverse(*d));

            // ---- further back: the ladder over everything the digest doesn't show.
            // Rung 0 is the materialized day table; the bands are a pure read-time
            // regrouping of it (obs counted from the evidence trail — the judged
            // lane is small). Frontier = the digest's oldest shown day.
            let all_days: std::collections::BTreeMap<u64, DayStats> =
                day_rows.iter().cloned().collect();
            let mut obs_per_day: std::collections::BTreeMap<u64, i64> = Default::default();
            for (subject, _) in subjects.iter() {
                for r in evidence.get(&subject) {
                    let r: ObsRow = r;
                    *obs_per_day
                        .entry(ladder::local_day(r.ts_ms, offset_ms))
                        .or_default() += 1;
                }
            }
            let frontier = match day_rows.len() {
                0 => today_bucket,
                n => day_rows[n.min(3) - 1].0,
            };
            let further = ladder::bands(&all_days, &obs_per_day, frontier, budget);
            let days_out: Vec<serde_json::Value> = day_rows
                .iter()
                .take(3)
                .map(|(day, s)| {
                    let mut fs: Vec<(&String, &i64)> = s.files.iter().collect();
                    fs.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
                    serde_json::json!({
                        "day": day_label(*day, today_bucket),
                        "tools": s.tools, "fails": s.fails,
                        "sessions": s.sessions,
                        "files": fs.iter().take(4).map(|(f, _)| short_path(f)).collect::<Vec<_>>(),
                    })
                })
                .collect();

            // ---- active now: sessions with activity in the last hour, by
            // worktree — one agent's brief sees what the others are doing
            let mut sess: Vec<(String, SessStats)> = sessions.iter().collect();
            sess.sort_by_key(|(_, s)| std::cmp::Reverse(s.end_ms));
            let active: Vec<serde_json::Value> = sess
                .iter()
                .filter(|(_, s)| now.saturating_sub(s.end_ms) < 3_600_000)
                .take(6)
                .map(|(id, s)| {
                    let place = s.cwd.rsplit('/').next().unwrap_or(&s.cwd);
                    serde_json::json!({
                        "where": place,
                        "session": short_sess(id),
                        "age": age_label(now, s.end_ms),
                    })
                })
                .collect();

            let last_session = last_session(&sess, desk).map(|(s, here)| {
                serde_json::json!({
                    "age": age_label(now, s.end_ms),
                    "branch": s.branch,
                    "final_msg": s.final_msg,
                    "where": s.cwd.rsplit('/').next().unwrap_or(&s.cwd),
                    "here": here,
                })
            });

            // ---- files: most-touched over the digest window, with their sessions
            let mut touch: HashMap<String, i64> = HashMap::new();
            for (_, s) in day_rows.iter().take(3) {
                for (f, n) in &s.files {
                    *touch.entry(f.clone()).or_default() += n;
                }
            }
            let mut touch: Vec<(String, i64)> = touch.into_iter().collect();
            touch.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let files_out: Vec<serde_json::Value> = touch
                .iter()
                .take(5)
                .map(|(path, _)| {
                    let mut ss = files.get(path);
                    ss.sort();
                    ss.dedup();
                    serde_json::json!({
                        "path": short_path(path),
                        "sessions": ss.iter().map(|s| short_sess(s)).collect::<Vec<_>>(),
                    })
                })
                .collect();

            // ---- relevant: hybrid RRF over the text indexes, disposition inline
            let mut relevant: Vec<serde_json::Value> = Vec::new();
            if !query.trim().is_empty() {
                let mut fused = rrf(
                    &kw.search(query, 12),
                    &vec.search(&ese::encode_single(query)),
                );
                // the calling session's rows rank after everyone else's
                // (stable, so each side keeps its fused order)
                let mine = |id: &EventId| calling.as_deref() == Some(id.0.as_str());
                fused.sort_by_key(|(id, _)| mine(id));
                let mut per_session: HashMap<String, usize> = HashMap::new();
                for (id, _) in fused {
                    if relevant.len() >= 6 {
                        break;
                    }
                    // one session must not flood the list with its user/final/obs
                    let n = per_session.entry(id.0.clone()).or_default();
                    if *n >= 2 {
                        continue;
                    }
                    let Some(t) = texts.get(&id) else { continue };
                    *n += 1;
                    let mut tag = t.kind.clone();
                    if t.kind == "obs" && t.cited {
                        tag.push_str("·cited");
                    }
                    let calling_session = mine(&id);
                    relevant.push(serde_json::json!({
                        "tag": format!(
                            "{tag} · {}{}",
                            age_label(now, t.ts_ms),
                            if calling_session { " · this session" } else { "" }
                        ),
                        "text": t.text,
                        "calling_session": calling_session,
                    }));
                }
            }

            Brief {
                // the caller's local calendar date, computed from the same civil-
                // days math as the rest of the tool (no subprocess)
                today: date_label((now as i64 + offset_ms) as u64),
                active,
                days: days_out,
                further,
                last_session,
                files: files_out,
                relevant,
            }
        },
    )
}

const DEFAULT_TMPL: &str = include_str!("../brief.tmpl");

/// Render through `.peat/brief.tmpl` if present (the experimentation
/// surface), else the embedded default. The template only formats — every
/// value is precomputed.
pub fn render(brief: &Brief) -> String {
    let tmpl = std::fs::read_to_string(crate::db::peat_dir().join("brief.tmpl"))
        .unwrap_or_else(|_| DEFAULT_TMPL.to_string());
    let mut env = minijinja::Environment::new();
    ui::add_style_filters(&mut env);
    env.add_template("brief", &tmpl).unwrap();
    env.get_template("brief")
        .unwrap()
        .render(minijinja::Value::from_serialize(brief))
        .unwrap_or_else(|e| format!("peat: template error: {e}\n"))
}

/// The one output branch: `--json` gets the full structure, a terminal
/// gets the template.
pub fn emit(brief: &Brief, json: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(brief).unwrap());
    } else {
        print!("{}", render(brief));
    }
}

fn day_label(bucket: u64, today: u64) -> String {
    match today.saturating_sub(bucket) {
        0 => "today".into(),
        1 => "yesterday".into(),
        n => format!("{n}d ago"),
    }
}

/// The brief's band budget: flag beats env beats default.
pub fn budget(flag: Option<usize>) -> usize {
    flag.or_else(|| {
        std::env::var("PEAT_BRIEF_BUDGET")
            .ok()
            .and_then(|v| v.parse().ok())
    })
    .unwrap_or(8)
}

/// This desk's root: the directory holding `.peat`.
pub fn desk() -> std::path::PathBuf {
    crate::db::peat_dir()
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| "/".into())
}

/// How recent this desk's last closed session must be for the agent brief
/// to hand on its closing message. Older work is better read on demand
/// than pushed: an old summary anchors a fresh session on old understanding.
const RESUME_WINDOW_MS: u64 = 3 * DAY_MS;

/// The one line every agent brief ends with: how to look the past up.
pub const LOOKUP_LINE: &str = "peat: to see the sessions behind a commit, change, bead, file or topic before you change it, run `peat <commit|change|bead-id|path|words>`.";

/// What `peat hook` injects at SessionStart: this desk's previous closing
/// message when it is recent and not this very session (a resume already
/// holds its own context), then the lookup line. Nothing else — counts,
/// claims and other desks' work are read on demand.
pub fn agent(st: &Db, desk: &std::path::Path, current: Option<&str>, now: u64) -> String {
    let mut sess: Vec<(String, SessStats)> =
        st.rtx(|(_, _, _, _, sessions, _)| sessions.iter().collect());
    sess.sort_by_key(|(_, s)| std::cmp::Reverse(s.end_ms));
    let mut out = String::new();
    // this desk's newest closed session; when that is the one starting (a
    // resume or compaction), it already holds its own context
    if let Some((s, true)) = last_session(&sess, desk)
        && let Some((id, _)) = sess.iter().find(|(_, x)| std::ptr::eq(x, s))
        && current.is_none_or(|c| c != id)
        && now.saturating_sub(s.end_ms) < RESUME_WINDOW_MS
    {
        out.push_str(&format!(
            "peat: this desk's last session ({} ago) closed with:\n  {}  ▸ peat {}\n",
            age_label(now, s.end_ms),
            ui::clip(&s.final_msg, 800),
            short_sess(id)
        ));
    }
    out.push_str(LOOKUP_LINE);
    out.push('\n');
    out
}
