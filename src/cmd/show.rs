//! `peat <session> [seq]`: one session's overview — its digest (what the
//! person asked, what ran and failed, the files it edited, its commits, how
//! it closed) and its observations — or one event in full with the
//! observations that cite it.

use crate::db::{self, Db};
use crate::event::{Envelope, EventId};
use crate::pipeline::{ObsRow, SessStats, SubjStats};
use crate::ui::{self, age_label, clip, short_path, short_sess};
use crate::vcs;

/// Files the text overview lists before pointing at `--json`.
const FILES_SHOWN: usize = 8;

pub fn show(st: &Db, session: String, seq: Option<u32>, json: bool) {
    let now = db::now_ms();
    let Some(seq) = seq else {
        overview(st, &session, now, json);
        return;
    };
    let (hit, citing) = st.rtx(|(_, _, _, (subjects, evidence), sessions, ledger)| {
        // resolve the session prefix against the small sessions
        // table, then point-read the ledger — never scan it
        let hit: Option<(EventId, Envelope)> = sessions
            .iter()
            .map(|(sess, _): (String, SessStats)| sess)
            .find(|sess| sess.starts_with(session.as_str()))
            .and_then(|sess| {
                let id = (sess, seq);
                ledger.get(&id).map(|e: Envelope| (id, e))
            });
        // the citer walk is only worth paying on a hit, and only
        // matching rows earn a subject-name clone
        let mut citing: Vec<(String, ObsRow)> = Vec::new();
        if let Some(((sess, _), _)) = &hit {
            for (name, _) in subjects.iter().collect::<Vec<(String, SubjStats)>>() {
                for r in evidence.get(&name) {
                    if r.session == *sess && r.derived_from.contains(&seq) {
                        citing.push((name.clone(), r));
                    }
                }
            }
        }
        (hit, citing)
    });
    match hit {
        None => {
            ui::error(&format!("no event ({session}*, {seq})"));
            std::process::exit(1);
        }
        Some(((sess, q), env)) if json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "session": sess, "seq": q, "v": env.v, "ts_ms": env.ts_ms,
                    "event": env.kind,
                    "cited_by": citing.iter().map(|(subj, r)| serde_json::json!({
                        "subject": subj, "seq": r.seq, "text": r.text,
                    })).collect::<Vec<_>>(),
                }))
                .unwrap()
            );
        }
        Some(((sess, q), env)) => {
            println!(
                "{} {}",
                ui::h1(&format!("event ({sess}, {q})")),
                ui::dim(&format!("· {} · v{}", age_label(now, env.ts_ms), env.v))
            );
            println!("{}", serde_json::to_string_pretty(&env.kind).unwrap());
            for (subj, r) in citing {
                println!(
                    "{} {}",
                    ui::dim(&format!(
                        "cited by obs [{subj} · {}]:",
                        age_label(now, r.ts_ms)
                    )),
                    r.text
                );
            }
        }
    }
}

/// The jj repo to read a session's commits from: its cwd while that
/// exists, else `desk` only when the cwd lay in one of this repo's
/// workspaces (a retired desk of the same repo). Anything else is `None`:
/// another repo's history would answer a confident, empty list.
pub fn session_repo(cwd: &str, desk: &std::path::Path) -> Option<std::path::PathBuf> {
    let cwd = std::path::Path::new(cwd);
    if cwd.is_dir() {
        return Some(cwd.to_path_buf());
    }
    vcs::workspace_roots(desk)
        .iter()
        .any(|root| cwd.starts_with(root))
        .then(|| desk.to_path_buf())
}

/// One session's overview: summary row, digest, observations, commits.
fn overview(st: &Db, session: &str, now: u64, json: bool) {
    let found = st.rtx(|(_, _, _, (subjects, evidence), sessions, _ledger)| {
        let hit: Option<(String, SessStats)> = sessions
            .iter()
            .find(|(sess, _): &(String, SessStats)| sess.starts_with(session));
        let mut obs: Vec<(String, ObsRow)> = Vec::new();
        if let Some((sess, _)) = &hit {
            for (name, _) in subjects.iter().collect::<Vec<(String, SubjStats)>>() {
                for r in evidence.get(&name) {
                    if r.session == *sess {
                        obs.push((name.clone(), r));
                    }
                }
            }
        }
        obs.sort_by_key(|(_, r)| r.ts_ms);
        (hit, obs)
    });
    let (Some((sess, s)), obs) = found else {
        ui::error(&format!("no session matching {session}*"));
        std::process::exit(1);
    };
    // commits come from jj at read time, by the `Session:` trailer — in the
    // session's own repo; `None` when jj cannot answer there
    let commits = session_repo(&s.cwd, &crate::brief::desk())
        .and_then(|repo| vcs::session_commits(&repo, &sess).ok());
    let short = short_sess(&sess);

    if json {
        // the row keeps a capped intent; the API form reads it whole
        let intent = s.intent_at.map(|(_, seq)| {
            let raw: Option<Envelope> =
                st.rtx(|(_, _, _, _, _, ledger)| ledger.get(&(sess.clone(), seq)));
            let text = match raw.map(|e| e.kind) {
                Some(crate::event::Event::UserMsg { text }) => {
                    crate::transcript::persons_words(&text).unwrap_or_else(|| s.intent.clone())
                }
                _ => s.intent.clone(),
            };
            serde_json::json!({ "seq": seq, "text": text })
        });
        let (commits, cut) = match &commits {
            Some((cs, cut)) => (
                Some(
                    cs.iter()
                        .map(|c| {
                            serde_json::json!({
                                "commit": c.commit_id, "change": c.change_id,
                                "subject": c.description.lines().next().unwrap_or("").trim(),
                            })
                        })
                        .collect::<Vec<_>>(),
                ),
                *cut,
            ),
            None => (None, false),
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "session": sess, "cwd": s.cwd, "branch": s.branch,
                "start_ms": s.start_ms, "end_ms": s.end_ms,
                "intent": intent,
                "tools": s.tools, "commands": s.commands, "fails": s.fails,
                "first_fails": s.first_fails.iter().map(|(_, seq, d)| serde_json::json!({
                    "seq": seq, "detail": d,
                })).collect::<Vec<_>>(),
                // every file, path order: what cross-seat overlap reads
                "files": s.files.iter().map(|(p, n)| serde_json::json!({
                    "path": p, "touches": n,
                })).collect::<Vec<_>>(),
                "commits": commits, "commits_cut": cut,
                "final_msg": s.final_msg,
                "observations": obs.iter().map(|(subj, r)| serde_json::json!({
                    "subject": subj, "seq": r.seq, "text": r.text,
                    "cited": !r.derived_from.is_empty(),
                })).collect::<Vec<_>>(),
            }))
            .unwrap()
        );
        return;
    }

    let place = s.cwd.rsplit('/').next().unwrap_or(&s.cwd);
    // branch is noise when it restates the worktree name
    let branch = if !s.branch.is_empty()
        && !s.branch.ends_with(place)
        && !place.ends_with(s.branch.as_str())
    {
        format!(" · {}", s.branch)
    } else {
        String::new()
    };
    println!(
        "{} {}",
        ui::h1(&format!("== session {short} ==")),
        ui::dim(&format!(
            "{place}{branch} · active {} – {} ago",
            age_label(now, s.start_ms),
            age_label(now, s.end_ms),
        ))
    );
    if let Some((_, seq)) = s.intent_at {
        println!(
            "\n{}\n  {} {}",
            ui::h1("asked:"),
            clip(&s.intent, 300),
            ui::dim(&format!("▸ peat {short} {seq}"))
        );
    }
    if s.tools > 0 {
        println!(
            "\n{} {}",
            ui::h1("ran:"),
            ui::dim(&format!(
                "{} tool calls · {} commands · {} failed",
                s.tools, s.commands, s.fails
            ))
        );
        for (_, seq, detail) in &s.first_fails {
            println!(
                "  {} {} {}",
                ui::warn("✗"),
                clip(detail, 120),
                ui::dim(&format!("▸ peat {short} {seq}"))
            );
        }
        let more = s.fails - s.first_fails.len() as i64;
        if more > 0 {
            println!(
                "  {}",
                ui::dim(&format!(
                    "… {more} more failed ▸ peat events --session {short} --kind tool"
                ))
            );
        }
    }
    if !s.files.is_empty() {
        let mut files: Vec<(&String, &i64)> = s.files.iter().collect();
        files.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        println!("\n{}", ui::h1(&format!("edited {} file(s):", files.len())));
        for (path, n) in files.iter().take(FILES_SHOWN) {
            // relative to where the session ran; elided when outside it
            let shown = std::path::Path::new(path.as_str())
                .strip_prefix(&s.cwd)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| short_path(path));
            println!("  {shown} {}", ui::dim(&format!("×{n}")));
        }
        if files.len() > FILES_SHOWN {
            println!(
                "  {}",
                ui::dim(&format!(
                    "… {} more ▸ peat {short} --json",
                    files.len() - FILES_SHOWN
                ))
            );
        }
    }
    if let Some((cs, cut)) = &commits
        && !cs.is_empty()
    {
        println!("\n{}", ui::h1("commits:"));
        for c in cs {
            let id = &c.commit_id[..c.commit_id.len().min(12)];
            println!(
                "  {} {} {}",
                ui::dim(&c.change_id[..c.change_id.len().min(8)]),
                clip(c.description.lines().next().unwrap_or(""), 80),
                ui::dim(&format!("▸ peat {id}"))
            );
        }
        if *cut {
            println!(
                "  {}",
                ui::dim(&format!(
                    "… newest {} shown ▸ jj log -r 'description(substring:\"Session: {sess}\")'",
                    vcs::LOOKUP_CAP
                ))
            );
        }
    }
    if !s.final_msg.is_empty() {
        println!("\n{}\n  {}", ui::h1("closing message:"), s.final_msg);
    }
    if !obs.is_empty() {
        println!("\n{}", ui::h1("observations:"));
        for (subj, r) in &obs {
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
                r.text
            );
        }
    }
    println!("\n{}", ui::dim(&format!("▸ peat events --session {short}")));
}
