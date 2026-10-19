//! `peat obs`: the one claim an agent records by hand. Shown only when
//! asked for (`peat subjects`, `peat <subject>`); nothing prompts for it.

use crate::db::{self, Db};
use crate::event::{Basis, Envelope, Event, OBS_SEQ_BASE};
use crate::pipeline::SubjStats;
use crate::ui;

/// Deposit one observation under `subject`.
pub fn obs(
    st: &mut Db,
    subject: String,
    text: Vec<String>,
    from: Vec<u32>,
    session: Option<String>,
    at: Option<String>,
) {
    let ts = match &at {
        None => db::now_ms(),
        Some(d) => match crate::transcript::local_day_ms(d, "12:00:00.000") {
            Some(ts) => ts,
            None => {
                ui::error(&format!("bad --at date {d:?}; expected YYYY-MM-DD"));
                std::process::exit(1);
            }
        },
    };
    let Some(session) = db::current_session(session) else {
        ui::error(
            "no session id (.peat/current-session missing here and \
beside the shared db); pass --session",
        );
        std::process::exit(1);
    };
    let text = text.join(" ");
    let env = Envelope::new(
        &session,
        ts,
        Event::Obs2 {
            subject: subject.clone(),
            text: text.clone(),
            derived_from: from,
            basis: deposit_basis(),
        },
    );
    // Advisory lint, never blocking: observations are read months
    // later by agents with zero shared context, so phrasing that
    // leans on the present moment quietly rots. The envelope
    // already carries the timestamp; the ledger already holds the
    // story (cite it with --from).
    let text_l = text.to_lowercase();
    for (marker, why) in [
        ("tonight", "time-deictic"),
        ("today", "time-deictic"),
        ("yesterday", "time-deictic"),
        ("this session", "session-deictic"),
        ("this evening", "time-deictic"),
        ("this morning", "time-deictic"),
        ("just now", "time-deictic"),
        ("earlier", "time-deictic"),
        ("the reviewer", "person-deictic"),
        ("this change", "change-deictic"),
        ("this fix", "change-deictic"),
        ("in flight", "status-log"),
        ("now live", "status-log"),
        ("mirrors ", "status-log"),
        ("mirrored to", "status-log"),
        ("shipped", "status-log"),
        ("landed", "status-log"),
    ] {
        if text_l.contains(marker) {
            ui::note(&format!(
                "style: {why:?} phrase {marker:?} — future readers lack this context; state the rule standalone (the timestamp is recorded, the story is citable via --from)"
            ));
        }
    }

    // one transaction: hint, seq scan, insert, and count all see
    // the same state (and the count sees our own write)
    let count = st.wtx(|tx| {
        let near: Vec<String> = tx.rtx(|(_, _, _, (subjects, _), _, _)| {
            subjects
                .iter()
                .filter(|(s, _): &(String, SubjStats)| {
                    s != &subject && (s.contains(&subject) || subject.contains(s.as_str()))
                })
                .map(|(s, v)| format!("{s} ({} obs)", v.count))
                .take(4)
                .collect()
        });
        if !near.is_empty() {
            ui::note(&format!("near subjects: {}", near.join(" · ")));
        }
        let mut seq = OBS_SEQ_BASE;
        while tx.contains(&(session.clone(), seq)) {
            seq += 1;
        }
        tx.upsert(&(session.clone(), seq), &env);
        tx.rtx(|(_, _, _, (subjects, _), _, _)| {
            subjects
                .get(&subject)
                .map(|s: SubjStats| s.count)
                .unwrap_or(1)
        })
    });
    // no checkpoint here: one observation is a few journal bytes,
    // and per-command rotation emits one-row SSTs across every
    // keyspace (L0 shredding — fjall stalls writes at 20 L0 runs).
    // Bulk capture checkpoints; the journal absorbs single events.
    if text.chars().count() > 240 {
        ui::note(
            "long for a claim — briefs clip at ~120 chars (trails read whole); \
consider splitting into separate observations",
        );
    }
    ui::note(&format!("recorded → {subject} (support {count})"));
}

/// Repo state at deposit time, stamped by construction — a convention the
/// depositor must remember is a check that gets deleted. Never fatal:
/// `None` outside a repo or without git. In a colocated jj repo, HEAD is
/// `@-` (the last named commit) — stable, unlike the working-copy commit.
fn deposit_basis() -> Option<Basis> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let commit = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if commit.is_empty() {
        return None;
    }
    // tracked files only: untracked noise (scratch dirs, data dirs that
    // self-ignore late) must not brand every deposit dirty
    let dirty = std::process::Command::new("git")
        .args(["status", "--porcelain", "-uno"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    Some(Basis { commit, dirty })
}
