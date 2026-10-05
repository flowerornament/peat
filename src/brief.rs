//! The brief: one snapshot folded into a session-start orientation.
//!
//! `--json` is the API and carries full text; the rendered form goes
//! through a template (`.peat/brief.tmpl` overrides the embedded default)
//! which owns all display-time clipping.

use std::collections::HashMap;

use fold::pipeline::Scored;
use fold::pipeline::terminal::{MultimapReader, TableReader};
use fold::stream::Readable;

use crate::event::{EventId, Lane};
use crate::ladder;
use crate::pipeline::{
    DAY_MS, DayStats, DistRow, DistStats, ObsRow, SessStats, SubjStats, TextRow, standing,
};
use crate::transcript::{date_label, local_offset_ms};
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

/// How many standing rulings and open loops the wake prints before it
/// points at the full register.
const RULINGS_SHOWN: usize = 20;
const LOOPS_SHOWN: usize = 12;
/// A loop nobody has touched for this long is more likely forgotten than
/// live. It stays open on the record (`peat loops`) but leaves the wake:
/// a list of stale reminders teaches its reader to skip the list.
const LOOP_FRESH_MS: u64 = 30 * DAY_MS;

/// Kinds worth pushing unasked: distilled or judged text, never the raw
/// firehose (user messages, mid-task chatter).
const PUSH_KINDS: [&str; 6] = ["ruling", "loop", "digest", "obs", "final", "compact"];

/// What to put in front of an agent the moment a prompt arrives: hits the
/// keyword and vector lanes *agree* on, from other sessions, not already
/// pushed. Agreement is the precision gate — an unasked line costs
/// attention, so one lane's guess is not enough.
#[allow(clippy::too_many_arguments)]
pub fn push_hits(
    prompt: &str,
    session: &str,
    kw_search: impl Fn(&str, usize) -> Vec<Scored<f64, EventId>>,
    vec_search: impl Fn(&[f32; ese::DIMENSIONS]) -> Vec<Scored<f32, EventId>>,
    text_of: impl Fn(&EventId) -> Option<TextRow>,
    seen: &std::collections::HashSet<EventId>,
    max: usize,
) -> Vec<(EventId, TextRow)> {
    let query = crate::event::cap(prompt, 600);
    let kw = kw_search(&query, 12);
    let vec = vec_search(&ese::encode_single(&query));
    let both = |id: &EventId| kw.iter().any(|h| &h.val == id) && vec.iter().any(|h| &h.val == id);
    let mut texts: std::collections::HashSet<String> = Default::default();
    let mut out = Vec::new();
    for (id, _) in rrf(&kw, &vec) {
        if out.len() >= max {
            break;
        }
        if !both(&id) || id.0 == session || seen.contains(&id) {
            continue;
        }
        let Some(t) = text_of(&id) else { continue };
        if PUSH_KINDS.contains(&t.kind.as_str()) && texts.insert(t.text.clone()) {
            out.push((id, t));
        }
    }
    out
}

/// The lines `peat hook` injects for [`push_hits`]: disposition first,
/// the read-the-whole-thing handle last.
pub fn push_text(hits: &[(EventId, TextRow)], now: u64) -> String {
    let mut s = String::from(
        "peat: from this project's memory, possibly relevant to this prompt \
(judge each; kind and age shown):",
    );
    for (id, t) in hits {
        s.push_str(&format!(
            "\n  [{} · {}] {}  ▸ peat {} {}",
            t.kind,
            age_label(now, t.ts_ms),
            ui::clip(&t.text, 320),
            short_sess(&id.0),
            id.1
        ));
    }
    s
}

#[derive(serde::Serialize)]
pub struct Brief {
    pub today: String,
    /// standing instructions from the user: hold until superseded
    rulings: Vec<serde_json::Value>,
    rulings_more: usize,
    /// unfinished things: hold until closed
    loops: Vec<serde_json::Value>,
    loops_more: usize,
    active: Vec<serde_json::Value>,
    days: Vec<serde_json::Value>,
    /// the temporal ladder: the rest of the past, geometrically coarser,
    /// every band carrying the command that descends into it
    further: Vec<ladder::Band>,
    last_session: Option<serde_json::Value>,
    files: Vec<serde_json::Value>,
    relevant: Vec<serde_json::Value>,
    subjects: Vec<serde_json::Value>,
}

/// Assemble the brief from one snapshot's readers. The two search indexes
/// arrive as closures (their reader types carry tokenizer/const params);
/// tables and the multimap come as concrete readers.
#[allow(clippy::too_many_arguments)]
pub fn assemble<R: Readable>(
    query: &str,
    now: u64,
    budget: usize,
    days: &TableReader<'_, R, u64, DayStats>,
    files: &MultimapReader<'_, R, String, String>,
    kw_search: impl Fn(&str, usize) -> Vec<Scored<f64, EventId>>,
    vec_search: impl Fn(&[f32; ese::DIMENSIONS]) -> Vec<Scored<f32, EventId>>,
    text_of: impl Fn(&EventId) -> Option<TextRow>,
    subjects: &TableReader<'_, R, String, SubjStats>,
    evidence: &MultimapReader<'_, R, String, ObsRow>,
    sessions: &TableReader<'_, R, String, SessStats>,
    distilled: &TableReader<'_, R, String, DistStats>,
    desk: &std::path::Path,
) -> Brief {
    let today_bucket = now / DAY_MS;

    // ---- the distilled lane, read once: digests by window key, and the
    // two registers that do not fade
    let dist: Vec<DistRow> = distilled.iter().filter_map(|(_, s)| s.row).collect();
    // window digests only: the bands and days look these up by handle
    let windows: HashMap<&str, &str> = dist
        .iter()
        .filter(|r| r.lane == Lane::Digest && !r.key.contains('#'))
        .map(|r| (r.key.as_str(), r.text.as_str()))
        .collect();
    let digest_of = |key: &str| windows.get(key).map(|t| (*t).to_string());
    let rulings = standing(&dist, Lane::Ruling);
    let rulings_out: Vec<serde_json::Value> = rulings
        .iter()
        .take(RULINGS_SHOWN)
        .map(|r| {
            serde_json::json!({
                "subject": r.key, "text": r.text, "age": age_label(now, r.ts_ms),
                // the citation is the expansion path: the user's own message
                "handle": r.cites.first().map(|(s, q)| format!("▸ peat {} {q}", short_sess(s))),
            })
        })
        .collect();
    let loops = standing(&dist, Lane::Loop);
    let fresh = |r: &&&DistRow| now.saturating_sub(r.ts_ms) <= LOOP_FRESH_MS;
    let loops_out: Vec<serde_json::Value> = loops
        .iter()
        .filter(fresh)
        .take(LOOPS_SHOWN)
        .map(|r| {
            serde_json::json!({
                "key": r.key, "text": r.text, "age": age_label(now, r.ts_ms),
                "handle": format!("▸ peat {}", short_sess(&r.session)),
            })
        })
        .collect();

    // ---- day digest: the 3 most recent non-empty days
    let mut day_rows: Vec<(u64, DayStats)> = days.iter().collect();
    day_rows.sort_by_key(|(d, _)| std::cmp::Reverse(*d));

    // ---- further back: the ladder over everything the digest doesn't show.
    // Rung 0 is the materialized day table; the bands are a pure read-time
    // regrouping of it (obs counted from the evidence trail — the judged
    // lane is small). Frontier = the digest's oldest shown day.
    let all_days: std::collections::BTreeMap<u64, DayStats> = day_rows.iter().cloned().collect();
    let mut obs_per_day: std::collections::BTreeMap<u64, i64> = Default::default();
    for (subject, _) in subjects.iter() {
        for r in evidence.get(&subject) {
            let r: ObsRow = r;
            *obs_per_day.entry(r.ts_ms / DAY_MS).or_default() += 1;
        }
    }
    let frontier = match day_rows.len() {
        0 => today_bucket,
        n => day_rows[n.min(3) - 1].0,
    };
    let mut further = ladder::bands(&all_days, &obs_per_day, frontier, budget);
    for b in &mut further {
        b.digest = b.handle.strip_prefix("peat ").and_then(digest_of);
    }
    let days_out: Vec<serde_json::Value> = day_rows
        .iter()
        .take(3)
        .map(|(day, s)| {
            let mut fs: Vec<(&String, &i64)> = s.files.iter().collect();
            fs.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
            let key = crate::distill::day_key(*day);
            serde_json::json!({
                "digest": digest_of(&key),
                "handle": format!("▸ peat {key}"),
                "day": day_label(*day, today_bucket),
                "tools": s.tools, "fails": s.fails,
                "commits": s.commits, "sessions": s.sessions,
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
                "commits": s.commits,
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
        let fused = rrf(
            &kw_search(query, 12),
            &vec_search(&ese::encode_single(query)),
        );
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
            let Some(t) = text_of(&id) else { continue };
            *n += 1;
            let mut tag = t.kind.clone();
            if t.kind == "obs" && t.cited {
                tag.push_str("·cited");
            }
            relevant.push(serde_json::json!({
                "tag": format!("{tag} · {}", age_label(now, t.ts_ms)),
                "text": t.text,
            }));
        }
    }

    // ---- subjects: current understanding, newest first
    let mut subj: Vec<(String, SubjStats)> = subjects.iter().collect();
    subj.sort_by_key(|(_, s)| std::cmp::Reverse(s.last_ms));
    let subjects_out: Vec<serde_json::Value> = subj
        .iter()
        .take(5)
        .map(|(name, s)| {
            serde_json::json!({
                "subject": name,
                "count": s.count,
                "cited": s.cited,
                "age": age_label(now, s.last_ms),
                // anchor disposition of the winning obs, pre-rendered:
                // the brief is a bounded read, not a data API
                "basis": s.basis.as_ref().map(|b| {
                    b.label_with(crate::pipeline::commits_since(&all_days, s.last_ms))
                }),
                "text": s.text,
                // the expansion path: briefs clip, trails read whole
                "handle": crate::subject_handle_pub(name),
            })
        })
        .collect();

    Brief {
        // the caller's local calendar date, computed from the same civil-
        // days math as the rest of the tool (no subprocess)
        today: date_label((now as i64 + local_offset_ms()) as u64),
        rulings_more: rulings.len().saturating_sub(rulings_out.len()),
        rulings: rulings_out,
        loops_more: loops.len().saturating_sub(loops_out.len()),
        loops: loops_out,
        active,
        days: days_out,
        further,
        last_session,
        files: files_out,
        relevant,
        subjects: subjects_out,
    }
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
