//! `peat <session> [seq]`: one session's overview, or one event in full
//! with the observations that cite it.

use crate::db::{self, Db};
use crate::event::{Envelope, EventId};
use crate::pipeline::{ObsRow, SessStats, SubjStats};
use crate::ui::{self, age_label, clip, short_sess};

pub fn show(st: &Db, session: String, seq: Option<u32>) {
    let now = db::now_ms();
    let Some(seq) = seq else {
        // no seq: one session's overview — summary row, its
        // observations, and the handle to its raw events
        let found = st.rtx(|(_, _, _, (subjects, evidence), sessions, _ledger)| {
            let hit: Option<(String, SessStats)> = sessions
                .iter()
                .find(|(sess, _): &(String, SessStats)| sess.starts_with(session.as_str()));
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
            ui::h1(&format!("== session {} ==", short_sess(&sess))),
            ui::dim(&format!(
                "{place}{branch} · active {} – {} ago · {} commits",
                age_label(now, s.start_ms),
                age_label(now, s.end_ms),
                s.commits
            ))
        );
        if !s.final_msg.is_empty() {
            println!(
                "\n{}\n  {}",
                ui::h1("closing message:"),
                clip(&s.final_msg, 400)
            );
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
        println!(
            "\n{}",
            ui::dim(&format!("▸ peat events --session {}", short_sess(&sess)))
        );
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
